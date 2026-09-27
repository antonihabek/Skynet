//! Role suggestion — and the word is *suggestion*.
//!
//! §63.4 draws the line and this module holds it: a URDF carries no semantics, so
//! `shoulder_pitch_joint` is *probably* `left_arm.shoulder.pitch`, and probably is not a basis for
//! cross-body remapping. A wrong role does not fail loudly — it silently binds a policy's elbow
//! channel to a wrist, and the first anyone knows is the sound.
//!
//! So the output of this module never reaches `joints[].role` on its own. It reaches the report a
//! human reads, and a human accepts it with a flag whose name says what they are doing.
//!
//! # The three answers, and none of them is a guess
//!
//! * **A suggestion** — every part resolved, and it parses under the canonical grammar.
//! * **A suggestion missing a side** — the joint is unmistakably a shoulder and the name never says
//!   whose. Common: a single-arm robot's URDF has no reason to write `left_`. Reported as such, and
//!   resolvable by an operator asserting the side, which is a thing an operator actually knows.
//! * **Nothing** — with the reason. `joint_7` gets no suggestion, and inventing `aux.link.rotate`
//!   for it would be worse than silence, because it would parse.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_contracts::parse_role;

use crate::model::{UrdfJoint, UrdfModel};

/// Which side of the body an operator asserts, when the file does not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The robot's left.
    Left,
    /// The robot's right.
    Right,
}

impl Side {
    /// The chain prefix.
    pub fn prefix(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }

    /// Parse a `--assume-side` value.
    pub fn parse(raw: &str) -> Option<Side> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "left" | "l" => Some(Side::Left),
            "right" | "r" => Some(Side::Right),
            _ => None,
        }
    }
}

/// How much weight the suggestion carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Every part came from a word in the joint's own name.
    Named,
    /// Something was inferred — an axis from the rotation vector, a side from an operator's
    /// assertion, an ordinal from position. Inference is where the wrong answers live.
    Inferred,
}

/// What this module concluded about one joint.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleSuggestion {
    /// The joint's wire name.
    pub joint: String,
    /// The suggested role, when one could be assembled and it parses.
    pub role: Option<String>,
    /// How it was arrived at. `None` when there is no suggestion.
    pub confidence: Option<Confidence>,
    /// What a reader needs to know to accept or reject it, in one sentence.
    pub reason: String,
    /// True when a side assertion is the only thing standing between this and a suggestion.
    pub needs_side: bool,
}

/// Chain keywords, longest first so `upper_arm` is not read as `arm` with a stray word.
///
/// The right-hand side is `(chain-without-side, link, needs-a-side)`. `needs_side` is what
/// separates "this is a shoulder and I do not know whose" from "this is the torso, and a torso has
/// no side".
const CHAIN_WORDS: &[(&str, &str, &str, bool)] = &[
    // word, chain (sideless chains give their full name), link, needs a side
    ("shoulder", "arm", "shoulder", true),
    ("upperarm", "arm", "upper", true),
    ("upper_arm", "arm", "upper", true),
    ("forearm", "arm", "forearm", true),
    ("elbow", "arm", "elbow", true),
    ("wrist", "arm", "wrist", true),
    ("thumb", "hand", "thumb", true),
    ("finger", "hand", "finger", true),
    ("gripper", "hand", "gripper", true),
    ("grip", "hand", "gripper", true),
    ("claw", "hand", "gripper", true),
    ("hip", "leg", "hip", true),
    ("knee", "leg", "knee", true),
    ("ankle", "leg", "ankle", true),
    ("thigh", "leg", "thigh", true),
    ("calf", "leg", "calf", true),
    ("foot", "leg", "foot", true),
    ("waist", "torso", "waist", false),
    ("torso", "torso", "waist", false),
    ("spine", "torso", "waist", false),
    ("chest", "torso", "chest", false),
    ("neck", "neck", "neck", false),
    ("head", "head", "head", false),
    ("wheel", "base", "wheel", false),
    ("caster", "base", "caster", false),
    ("tail", "tail", "tail", false),
];

/// Axis words, and the canonical axis each maps to.
///
/// `pan`/`tilt` and `lift` are the interesting rows. They are pan-tilt-unit and hobby-servo-arm
/// vocabulary respectively — `shoulder_pan` and `shoulder_lift` are the SO-101's own names — and
/// they mean yaw and pitch. They are still marked inferred, because "lift" naming a pitch is a
/// convention rather than a statement.
const AXIS_WORDS: &[(&str, &str, bool)] = &[
    // word, canonical axis, came-from-the-name (rather than a convention)
    ("pitch", "pitch", true),
    ("yaw", "yaw", true),
    ("roll", "roll", true),
    ("flex", "flex", true),
    ("abduct", "abduct", true),
    ("abduction", "abduct", true),
    ("abad", "abduct", true),
    ("rotate", "rotate", true),
    ("rot", "rotate", true),
    ("slide", "slide", true),
    ("grip", "grip", true),
    ("pan", "yaw", false),
    ("tilt", "pitch", false),
    ("lift", "pitch", false),
];

/// Suggest a role for every actuated joint in the model.
///
/// Returns one entry per actuator, in the model's own order, including the ones with no suggestion
/// — because "eight joints, five suggested" is the number a reader needs, and a filtered list hides
/// the three.
pub fn suggest_roles(model: &UrdfModel, assume_side: Option<Side>) -> Vec<RoleSuggestion> {
    let mut out: Vec<RoleSuggestion> = Vec::new();
    for joint in model.actuators() {
        out.push(suggest_one(joint, body_axis(model, joint), assume_side));
    }
    disambiguate_duplicates(&mut out);
    out
}

fn suggest_one(
    joint: &UrdfJoint,
    axis_in_body: [f64; 3],
    assume_side: Option<Side>,
) -> RoleSuggestion {
    let tokens = tokenise(&joint.name);
    let none = |reason: String| RoleSuggestion {
        joint: joint.name.clone(),
        role: None,
        confidence: None,
        reason,
        needs_side: false,
    };

    let Some((word, chain_stem, link, needs_side)) = find_chain(&tokens) else {
        return none(format!(
            "no word in `{}` names a body part the vocabulary knows, so there is nothing to \
             suggest. Assign a role by hand if this joint has a cross-body meaning.",
            joint.name
        ));
    };

    let named_side = find_side(&tokens);
    let (chain, side_inferred) = if needs_side {
        match named_side.or(assume_side) {
            Some(side) => (
                format!("{}_{}", side.prefix(), chain_stem),
                named_side.is_none(),
            ),
            None => {
                return RoleSuggestion {
                    joint: joint.name.clone(),
                    role: None,
                    confidence: None,
                    reason: format!(
                        "`{}` names {} {word}, and nothing in the name says which side. Assert it \
                         with --assume-side left|right, or assign the role by hand.",
                        joint.name,
                        article(word)
                    ),
                    needs_side: true,
                }
            }
        }
    } else {
        (chain_stem.to_string(), false)
    };

    let (axis, axis_named) = match find_axis(&tokens) {
        Some((axis, from_name)) => (Some(axis.to_string()), from_name),
        None => (axis_from_geometry(joint, axis_in_body), false),
    };

    let mut role = format!("{chain}.{link}");
    if let Some(axis) = &axis {
        role.push('.');
        role.push_str(axis);
    }

    // A suggestion that does not parse is not a suggestion. This is the same rule §5.1 states for
    // the vocabulary itself, applied to our own output before anyone sees it.
    if parse_role(Some(&role)).is_none() {
        return none(format!(
            "the name suggests `{role}`, which does not parse under the canonical grammar, so it \
             is not offered."
        ));
    }

    let confidence = if side_inferred || !axis_named {
        Confidence::Inferred
    } else {
        Confidence::Named
    };

    // Built as clauses rather than one appended sentence, so that the caveats read as caveats
    // instead of running into the claim they qualify.
    let mut clauses: Vec<String> = Vec::new();
    clauses.push(match &axis {
        Some(axis) => format!(
            "`{}` names {} {word} on the {axis} axis",
            joint.name,
            article(word)
        ),
        None => format!(
            "`{}` names {} {word}, and nothing names its axis",
            joint.name,
            article(word)
        ),
    });
    if side_inferred {
        clauses.push("the side was asserted rather than read from the name".to_string());
    }
    if axis.is_some() && !axis_named {
        clauses.push("the axis was inferred from the rotation vector, not the name".to_string());
    }
    let reason = format!("{}.", clauses.join("; "));

    RoleSuggestion {
        joint: joint.name.clone(),
        role: Some(role),
        confidence: Some(confidence),
        reason,
        needs_side: false,
    }
}

/// Two joints cannot share a role, so repeats get an ordinal — and the ordinal is *positional*,
/// which is exactly the sort of thing that is right until someone reorders the file.
fn disambiguate_duplicates(suggestions: &mut [RoleSuggestion]) {
    let roles: Vec<Option<String>> = suggestions.iter().map(|s| s.role.clone()).collect();
    for index in 0..suggestions.len() {
        let Some(role) = roles[index].clone() else {
            continue;
        };
        let same: Vec<usize> = roles
            .iter()
            .enumerate()
            .filter(|(_, other)| other.as_deref() == Some(role.as_str()))
            .map(|(i, _)| i)
            .collect();
        if same.len() < 2 {
            continue;
        }
        let ordinal = same.iter().position(|&i| i == index).unwrap_or(0);
        suggestions[index].role = Some(format!("{role}#{ordinal}"));
        suggestions[index].confidence = Some(Confidence::Inferred);
        suggestions[index].reason = format!(
            "{} {} joints suggest `{role}`; this one was numbered `#{ordinal}` by its position in \
             the file, which is a guess about which is which.",
            suggestions[index].reason,
            same.len()
        );
    }
}

/// "a" or "an". Small, and the alternative is a tool that says "names a elbow" in the one place a
/// reader is being asked to weigh its judgement about names.
fn article(word: &str) -> &'static str {
    match word.chars().next() {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    }
}

fn tokenise(name: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            // A camelCase boundary is a word boundary: `shoulderPitch` is two words.
            if ch.is_uppercase() && previous_lower && !current.is_empty() {
                tokens.push(core::mem::take(&mut current));
            }
            current.push(ch.to_ascii_lowercase());
            previous_lower = ch.is_lowercase() || ch.is_numeric();
        } else if !current.is_empty() {
            tokens.push(core::mem::take(&mut current));
            previous_lower = false;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn find_chain(tokens: &[String]) -> Option<(&'static str, &'static str, &'static str, bool)> {
    // Whole-token matches first, then substrings, so `forearm_roll` is a forearm rather than an arm.
    for (word, chain, link, needs_side) in CHAIN_WORDS {
        if tokens.iter().any(|token| token == word) {
            return Some((word, chain, link, *needs_side));
        }
    }
    for (word, chain, link, needs_side) in CHAIN_WORDS {
        if tokens.iter().any(|token| token.contains(word)) {
            return Some((word, chain, link, *needs_side));
        }
    }
    // `arm` and `base` are last: they are the words most likely to appear as part of something
    // else, and they carry the least information when they do.
    if tokens.iter().any(|token| token == "arm") {
        return Some(("arm", "arm", "link", true));
    }
    if tokens.iter().any(|token| token == "base") {
        return Some(("base", "base", "link", false));
    }
    None
}

fn find_side(tokens: &[String]) -> Option<Side> {
    for token in tokens {
        match token.as_str() {
            "left" | "l" | "lh" | "lf" => return Some(Side::Left),
            "right" | "r" | "rh" | "rf" => return Some(Side::Right),
            _ => {}
        }
    }
    None
}

fn find_axis(tokens: &[String]) -> Option<(&'static str, bool)> {
    for (word, axis, from_name) in AXIS_WORDS {
        if tokens.iter().any(|token| token == word) {
            return Some((axis, *from_name));
        }
    }
    for (word, axis, from_name) in AXIS_WORDS {
        if tokens.iter().any(|token| token.contains(word)) {
            return Some((axis, *from_name));
        }
    }
    None
}

/// Last resort: read the axis off the rotation vector, in the body's frame.
///
/// REP-103 body axes (§44.4) — x forward, y left, z up — make rotation about z a yaw, about y a
/// pitch and about x a roll. A URDF joint axis is written in the joint's own frame, after every
/// origin rotation above it, so the body-frame axis is what is read: [`body_axis`] composes the
/// chain at the zero pose. Reading the joint's own frame, as this did until `DRY-COVERAGE-PLAN.md`
/// F147, called every joint of a Universal Robots arm a yaw — the elbow and both wrist pitches
/// included — because UR writes each axis as `0 0 1` in a frame rotated to it. It is still
/// inference, labelled as such, and offered only when the axis is unambiguously one cardinal
/// direction; a body whose zero pose is not REP-103's gets the zero pose's answer.
fn axis_from_geometry(joint: &UrdfJoint, axis_in_body: [f64; 3]) -> Option<String> {
    if joint.joint_type == "prismatic" {
        return Some("slide".to_string());
    }
    let [x, y, z] = axis_in_body.map(f64::abs);
    let dominant = 0.999;
    let negligible = 0.001;
    if x > dominant && y < negligible && z < negligible {
        Some("roll".to_string())
    } else if y > dominant && x < negligible && z < negligible {
        Some("pitch".to_string())
    } else if z > dominant && x < negligible && y < negligible {
        Some("yaw".to_string())
    } else {
        None
    }
}

/// A joint's axis in the root link's frame at the zero pose: every origin rotation from the root
/// down to this joint, applied to the axis the file writes in the joint's own frame.
///
/// A chain that does not reach the root — which a well-formed tree never has — stops where it
/// breaks, and the depth bound makes a cycle end rather than loop.
fn body_axis(model: &UrdfModel, joint: &UrdfJoint) -> [f64; 3] {
    let mut axis = rotate(rpy_matrix(joint.origin_rpy), joint.axis);
    let mut link = joint.parent.as_str();
    for _ in 0..model.joints.len() {
        let Some(above) = model
            .joints
            .iter()
            .find(|candidate| candidate.child == link)
        else {
            break;
        };
        axis = rotate(rpy_matrix(above.origin_rpy), axis);
        link = above.parent.as_str();
    }
    axis
}

/// URDF's `rpy`: extrinsic X, Y, Z, so `Rz(yaw) · Ry(pitch) · Rx(roll)` — the composition
/// `descriptor::quaternion` uses for the same angles.
fn rpy_matrix([roll, pitch, yaw]: [f64; 3]) -> [[f64; 3]; 3] {
    let (sr, cr) = (roll.sin(), roll.cos());
    let (sp, cp) = (pitch.sin(), pitch.cos());
    let (sy, cy) = (yaw.sin(), yaw.cos());
    [
        [cy * cp, cy * sp * sr - sy * cr, cy * sp * cr + sy * sr],
        [sy * cp, sy * sp * sr + cy * cr, sy * sp * cr - cy * sr],
        [-sp, cp * sr, cp * cr],
    ]
}

fn rotate(matrix: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;
    use alloc::vec;

    fn model_with(joints: &[(&str, &str, &str)]) -> UrdfModel {
        let mut xml = String::from("<robot name=\"t\"><link name=\"root\"/>");
        for (index, (name, joint_type, axis)) in joints.iter().enumerate() {
            xml.push_str(&format!("<link name=\"l{index}\"/>"));
            xml.push_str(&format!(
                "<joint name=\"{name}\" type=\"{joint_type}\"><parent link=\"root\"/>\
                 <child link=\"l{index}\"/><axis xyz=\"{axis}\"/>\
                 <limit lower=\"-1\" upper=\"1\" effort=\"1\" velocity=\"1\"/></joint>"
            ));
        }
        xml.push_str("</robot>");
        model::parse(&xml).unwrap()
    }

    #[test]
    fn a_fully_named_joint_is_suggested_from_its_name_alone() {
        let model = model_with(&[("left_shoulder_pitch_joint", "revolute", "0 1 0")]);
        let suggestions = suggest_roles(&model, None);
        assert_eq!(
            suggestions[0].role.as_deref(),
            Some("left_arm.shoulder.pitch")
        );
        assert_eq!(suggestions[0].confidence, Some(Confidence::Named));
    }

    /// The SO-101's own joint names, which §67.2 maps by hand. The tool reaches the same answer for
    /// five of six and says out loud that the side came from the operator, not the file.
    #[test]
    fn the_so101_names_resolve_once_a_side_is_asserted() {
        let model = model_with(&[
            ("shoulder_pan", "revolute", "0 0 1"),
            ("shoulder_lift", "revolute", "0 1 0"),
            ("elbow_flex", "revolute", "0 1 0"),
            ("wrist_flex", "revolute", "0 1 0"),
            ("wrist_roll", "revolute", "1 0 0"),
            ("gripper", "revolute", "0 1 0"),
        ]);

        let without = suggest_roles(&model, None);
        assert!(without.iter().all(|s| s.role.is_none()));
        assert!(without.iter().all(|s| s.needs_side));
        assert!(without[0].reason.contains("--assume-side"));

        let with = suggest_roles(&model, Some(Side::Right));
        let roles: Vec<Option<&str>> = with.iter().map(|s| s.role.as_deref()).collect();
        assert_eq!(
            roles,
            vec![
                Some("right_arm.shoulder.yaw"),
                Some("right_arm.shoulder.pitch"),
                Some("right_arm.elbow.flex"),
                Some("right_arm.wrist.flex"),
                Some("right_arm.wrist.roll"),
                Some("right_hand.gripper.grip"),
            ]
        );
        // Every one of them is inferred, because the side was asserted rather than named.
        assert!(with
            .iter()
            .all(|s| s.confidence == Some(Confidence::Inferred)));
    }

    #[test]
    fn a_joint_the_vocabulary_cannot_place_gets_nothing_and_says_why() {
        let model = model_with(&[("joint_7", "revolute", "0 0 1")]);
        let suggestions = suggest_roles(&model, Some(Side::Left));
        assert_eq!(suggestions[0].role, None);
        assert!(!suggestions[0].needs_side);
        assert!(
            suggestions[0].reason.contains("nothing to suggest"),
            "{}",
            suggestions[0].reason
        );
    }

    #[test]
    fn a_sideless_chain_needs_no_side_assertion() {
        let model = model_with(&[
            ("torso_yaw", "revolute", "0 0 1"),
            ("neck_pitch", "revolute", "0 1 0"),
            ("wheel_left", "continuous", "0 1 0"),
        ]);
        let suggestions = suggest_roles(&model, None);
        assert_eq!(suggestions[0].role.as_deref(), Some("torso.waist.yaw"));
        assert_eq!(suggestions[1].role.as_deref(), Some("neck.neck.pitch"));
        assert_eq!(suggestions[2].role.as_deref(), Some("base.wheel.pitch"));
    }

    /// Two joints suggesting one role is the case that would silently bind two channels to the same
    /// place. Numbering them is the least-bad answer, and it is labelled a guess.
    #[test]
    fn duplicate_suggestions_are_numbered_and_downgraded() {
        let model = model_with(&[
            ("left_finger_flex", "revolute", "0 1 0"),
            ("left_finger_flex_b", "revolute", "0 1 0"),
        ]);
        let suggestions = suggest_roles(&model, None);
        assert_eq!(
            suggestions[0].role.as_deref(),
            Some("left_hand.finger.flex#0")
        );
        assert_eq!(
            suggestions[1].role.as_deref(),
            Some("left_hand.finger.flex#1")
        );
        assert!(suggestions[0]
            .reason
            .contains("by its position in the file"));
        assert_eq!(suggestions[0].confidence, Some(Confidence::Inferred));
    }

    /// The geometric fallback, and the honesty about it. A prismatic joint slides; a revolute one
    /// about a cardinal axis reads as roll, pitch or yaw and says the name did not tell us.
    #[test]
    fn the_axis_can_come_from_the_geometry_and_is_labelled_when_it_does() {
        let model = model_with(&[
            ("torso_lin", "prismatic", "0 0 1"),
            ("neck_a", "revolute", "0 0 1"),
            ("neck_b", "revolute", "0.5 0.5 0.7"),
        ]);
        let suggestions = suggest_roles(&model, None);
        assert_eq!(suggestions[0].role.as_deref(), Some("torso.waist.slide"));
        assert_eq!(suggestions[1].role.as_deref(), Some("neck.neck.yaw"));
        assert!(suggestions[1]
            .reason
            .contains("inferred from the rotation vector"));
        // An axis pointing nowhere in particular gets no axis at all rather than the nearest one.
        assert_eq!(suggestions[2].role.as_deref(), Some("neck.neck"));
        assert!(suggestions[2].reason.contains("nothing names its axis"));
    }

    /// F147. Universal Robots writes every axis as `0 0 1` in a frame rotated to it, so the joint's
    /// own frame calls the elbow a yaw. Read in the body's frame, a quarter turn about x under it
    /// makes it the pitch it is — and the rotation composes down the chain, not just one level.
    #[test]
    fn the_axis_is_read_in_the_body_frame_not_the_joints_own() {
        let xml = r#"<robot name="ur-shaped">
            <link name="base"/><link name="shoulder"/><link name="upper"/><link name="fore"/>
            <joint name="shoulder_pan_joint" type="revolute"><parent link="base"/>
              <child link="shoulder"/><axis xyz="0 0 1"/>
              <limit lower="-6" upper="6" effort="1" velocity="1"/></joint>
            <joint name="shoulder_lift_joint" type="revolute"><parent link="shoulder"/>
              <child link="upper"/><origin rpy="1.5707963267948966 0 0"/><axis xyz="0 0 1"/>
              <limit lower="-6" upper="6" effort="1" velocity="1"/></joint>
            <joint name="elbow_joint" type="revolute"><parent link="upper"/>
              <child link="fore"/><axis xyz="0 0 1"/>
              <limit lower="-3" upper="3" effort="1" velocity="1"/></joint>
          </robot>"#;
        let model = model::parse(xml).unwrap();
        let roles: Vec<Option<String>> = suggest_roles(&model, Some(Side::Right))
            .into_iter()
            .map(|suggestion| suggestion.role)
            .collect();
        assert_eq!(
            roles,
            vec![
                Some("right_arm.shoulder.yaw".to_string()),
                Some("right_arm.shoulder.pitch".to_string()),
                Some("right_arm.elbow.pitch".to_string()),
            ],
            "the elbow sits under the lift's quarter turn, so it is a pitch in the body frame"
        );
    }

    #[test]
    fn camel_case_is_a_word_boundary() {
        assert_eq!(
            tokenise("leftShoulderPitch"),
            vec!["left", "shoulder", "pitch"]
        );
        assert_eq!(tokenise("J1_wrist-roll"), vec!["j1", "wrist", "roll"]);
    }

    #[test]
    fn a_named_side_beats_an_asserted_one() {
        let model = model_with(&[("l_elbow_flex", "revolute", "0 1 0")]);
        let suggestions = suggest_roles(&model, Some(Side::Right));
        assert_eq!(suggestions[0].role.as_deref(), Some("left_arm.elbow.flex"));
        assert_eq!(suggestions[0].confidence, Some(Confidence::Named));
    }

    #[test]
    fn mimic_joints_are_not_suggested_because_they_are_not_commandable() {
        let model = model::parse(
            r#"<robot name="g"><link name="a"/><link name="b"/><link name="c"/>
               <joint name="left_finger_flex" type="prismatic"><parent link="a"/><child link="b"/>
                 <limit lower="0" upper="1" effort="1" velocity="1"/></joint>
               <joint name="left_finger_flex_mirror" type="prismatic"><parent link="a"/>
                 <child link="c"/><mimic joint="left_finger_flex"/>
                 <limit lower="0" upper="1" effort="1" velocity="1"/></joint></robot>"#,
        )
        .unwrap();
        let suggestions = suggest_roles(&model, None);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].joint, "left_finger_flex");
    }
}

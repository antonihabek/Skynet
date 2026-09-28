//! The canonical role vocabulary.
//!
//! This is the mechanism that makes cross-body work possible at all, and it is deliberately
//! modest. Wire names stay vendor-specific; the *semantics* become shared. It is the USB
//! descriptor move, and it is the smallest change that lets two vendors' naming schemes be
//! related mechanically instead of by an engineer reading two URDFs side by side.
//!
//! ```text
//! role  := chain "." link [ "." axis ] [ "#" index ]
//!
//! left_arm.shoulder.pitch    right_hand.finger.flex#2    base.wheel#0
//! ```
//!
//! # The extensibility rule is the important part
//!
//! [`parse_role`] returns `None` for anything it does not recognise, and a joint with an
//! unparseable role can only ever be matched **by exact name**. An unrecognised role never
//! silently maps to something plausible. A body the vocabulary does not describe stays usable — it
//! just cannot participate in remapping, which is truthful, because nobody has established what
//! its joints mean.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::js::js_trim;

/// Kinematic chains a role may belong to.
///
/// Deliberately small and body-plan agnostic. `aux` exists so a body plan nobody anticipated — a
/// snake robot's segments, a second mast, a sensor boom — can still declare roles that parse,
/// rather than being pushed into a wrong-but-close chain because the vocabulary had no honest slot
/// for it.
pub const CANONICAL_CHAINS: [&str; 12] = [
    "left_arm",
    "right_arm",
    "left_leg",
    "right_leg",
    "left_hand",
    "right_hand",
    "torso",
    "neck",
    "head",
    "base",
    "tail",
    "aux",
];

/// Axis of motion, when a link has more than one degree of freedom.
///
/// A three-DoF shoulder is three joints in every real driver, and the axis suffix is what keeps
/// `left_arm.shoulder.pitch` from colliding with `left_arm.shoulder.roll` while still letting both
/// be recognised as the same shoulder on a different robot.
pub const CANONICAL_AXES: [&str; 8] = [
    "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip",
];

/// A role that parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRole {
    /// One of [`CANONICAL_CHAINS`].
    pub chain: String,
    /// The link name. Vendor-chosen within the charset, not drawn from a fixed list.
    pub link: String,
    /// One of [`CANONICAL_AXES`], when the role names one.
    pub axis: Option<String>,
    /// Ordinal for repeated links — `base.wheel#2`, `right_hand.finger#1`. On a leg chain
    /// it counts legs from the front (§94): `left_leg.hip.abduct#0` is the front-left leg.
    pub index: Option<u32>,
}

impl ParsedRole {
    /// Canonical string form, so two parsed roles can be compared as keys.
    pub fn key(&self) -> String {
        let mut out = String::with_capacity(self.chain.len() + self.link.len() + 16);
        out.push_str(&self.chain);
        out.push('.');
        out.push_str(&self.link);
        if let Some(axis) = &self.axis {
            out.push('.');
            out.push_str(axis);
        }
        if let Some(index) = self.index {
            out.push('#');
            out.push_str(&index.to_string());
        }
        out
    }
}

fn is_chain_charset(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn is_link_charset(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn is_axis_charset(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_lowercase())
}

/// Parse a canonical role.
///
/// Returns `None` rather than erroring, and the caller treats `None` as "this joint can only be
/// matched by exact name" — see the module note.
///
/// Input is trimmed and lowercased first, matching the reference implementation. The index is
/// bounded to a `u32`: a joint ordinal above four billion is not a joint ordinal, and refusing it
/// is better than carrying a value no consumer of the key could render identically.
pub fn parse_role(role: Option<&str>) -> Option<ParsedRole> {
    let role = role?;
    if role.is_empty() {
        return None;
    }
    let normalised = js_trim(role).to_lowercase();
    if normalised.is_empty() {
        return None;
    }

    // `#` splits the ordinal off the end. Everything after it must be digits, so a second `#`
    // fails here rather than being absorbed into a link name.
    let (head, index) = match normalised.split_once('#') {
        Some((head, tail)) => {
            if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            match tail.parse::<u32>() {
                Ok(value) => (head, Some(value)),
                Err(_) => return None,
            }
        }
        None => (normalised.as_str(), None),
    };

    let mut parts = head.split('.');
    let chain = parts.next()?;
    let link = parts.next()?;
    let axis = parts.next();
    if parts.next().is_some() {
        // A fourth segment. The grammar has three.
        return None;
    }

    if !is_chain_charset(chain) || !is_link_charset(link) {
        return None;
    }
    if let Some(axis) = axis {
        if !is_axis_charset(axis) {
            return None;
        }
    }

    if !CANONICAL_CHAINS.contains(&chain) {
        return None;
    }
    if let Some(axis) = axis {
        if !CANONICAL_AXES.contains(&axis) {
            return None;
        }
    }

    Some(ParsedRole {
        chain: chain.to_string(),
        link: link.to_string(),
        axis: axis.map(ToString::to_string),
        index,
    })
}

/// Parse and immediately reduce to a key, for the common "can these two be related?" question.
pub fn role_key_of(role: Option<&str>) -> Option<String> {
    parse_role(role).map(|parsed| parsed.key())
}

/// The chains a body's joint roles put it on, in [`CANONICAL_CHAINS`] order (§87.3).
///
/// **Vocabulary order rather than declaration order**, which is the opposite of what the rest of
/// this crate does with a body's own lists, and the reason is that this list is read as a *set*: it
/// is what §87.3 measures a per-chain `on_command_loss` declaration's coverage against, and a
/// refusal that named *"the first uncovered chain"* would name a different chain depending on which
/// joint a vendor happened to write first. Coverage is a property of the body; the order it is
/// reported in should be a property of the vocabulary.
///
/// A joint whose role does not parse is on **no** chain. §5.1 makes that a usable body rather than
/// a broken one — it simply cannot participate in remapping — and §87.3's second finding is what
/// falls out of it: such a body cannot declare per chain at all, because the declaration would
/// cover nothing.
pub fn chains_of<'a, I: IntoIterator<Item = Option<&'a str>>>(roles: I) -> Vec<&'static str> {
    let mut present: Vec<&'static str> = Vec::new();
    for role in roles {
        let Some(parsed) = parse_role(role) else {
            continue;
        };
        if let Some(chain) = CANONICAL_CHAINS
            .iter()
            .find(|known| **known == parsed.chain.as_str())
        {
            if !present.contains(chain) {
                present.push(chain);
            }
        }
    }
    CANONICAL_CHAINS
        .iter()
        .copied()
        .filter(|chain| present.contains(chain))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_segment_form() {
        let parsed = parse_role(Some("left_arm.shoulder.pitch")).unwrap();
        assert_eq!(parsed.chain, "left_arm");
        assert_eq!(parsed.link, "shoulder");
        assert_eq!(parsed.axis.as_deref(), Some("pitch"));
        assert_eq!(parsed.index, None);
        assert_eq!(parsed.key(), "left_arm.shoulder.pitch");
    }

    #[test]
    fn parses_the_two_segment_form() {
        let parsed = parse_role(Some("torso.yaw")).unwrap();
        assert_eq!(
            parsed.link, "yaw",
            "the second segment is the link, not the axis"
        );
        assert_eq!(parsed.axis, None);
        assert_eq!(parsed.key(), "torso.yaw");
    }

    #[test]
    fn parses_ordinals() {
        let parsed = parse_role(Some("right_hand.finger.flex#2")).unwrap();
        assert_eq!(parsed.index, Some(2));
        assert_eq!(parsed.key(), "right_hand.finger.flex#2");

        let wheel = parse_role(Some("base.wheel#0")).unwrap();
        assert_eq!(wheel.index, Some(0));
        assert_eq!(wheel.key(), "base.wheel#0");
    }

    #[test]
    fn normalises_case_and_surrounding_space() {
        assert_eq!(
            parse_role(Some("  LEFT_ARM.Shoulder.PITCH "))
                .unwrap()
                .key(),
            "left_arm.shoulder.pitch"
        );
    }

    #[test]
    fn an_unrecognised_chain_is_none_rather_than_a_guess() {
        assert_eq!(parse_role(Some("tentacle.segment.flex")), None);
        assert_eq!(parse_role(Some("left_wing.shoulder.pitch")), None);
    }

    #[test]
    fn an_unrecognised_axis_is_none() {
        assert_eq!(parse_role(Some("left_arm.shoulder.wiggle")), None);
    }

    #[test]
    fn aux_gives_an_unanticipated_body_plan_an_honest_slot() {
        let parsed = parse_role(Some("aux.mast.pitch")).unwrap();
        assert_eq!(parsed.chain, "aux");
    }

    #[test]
    fn malformed_roles_are_none() {
        assert_eq!(parse_role(None), None);
        assert_eq!(parse_role(Some("")), None);
        assert_eq!(parse_role(Some("   ")), None);
        assert_eq!(parse_role(Some("left_arm")), None, "one segment");
        assert_eq!(parse_role(Some("a.b.c.d")), None, "four segments");
        assert_eq!(parse_role(Some("left_arm..pitch")), None, "empty link");
        assert_eq!(
            parse_role(Some("left_arm.shoulder.pitch#")),
            None,
            "empty index"
        );
        assert_eq!(
            parse_role(Some("left_arm.shoulder.pitch#a")),
            None,
            "non-numeric index"
        );
        assert_eq!(
            parse_role(Some("left_arm.shoulder.pitch#1#2")),
            None,
            "two ordinals"
        );
        assert_eq!(
            parse_role(Some("left_arm.shoulder-x.pitch")),
            None,
            "hyphen"
        );
        assert_eq!(
            parse_role(Some("left_arm.shoulder.pitch2")),
            None,
            "digit in axis"
        );
    }

    #[test]
    fn digits_are_allowed_in_a_link_but_not_a_chain() {
        assert!(parse_role(Some("base.wheel2")).is_some());
        assert_eq!(parse_role(Some("base2.wheel")), None);
    }

    #[test]
    fn an_ordinal_too_large_to_render_identically_is_refused() {
        assert_eq!(parse_role(Some("base.wheel#99999999999999")), None);
    }

    #[test]
    fn every_chain_and_axis_in_the_vocabulary_parses() {
        for chain in CANONICAL_CHAINS {
            let role = alloc::format!("{chain}.link");
            assert!(parse_role(Some(&role)).is_some(), "{chain}");
        }
        for axis in CANONICAL_AXES {
            let role = alloc::format!("torso.link.{axis}");
            assert!(parse_role(Some(&role)).is_some(), "{axis}");
        }
    }

    #[test]
    fn the_key_is_a_fixed_point() {
        for role in [
            "left_arm.shoulder.pitch",
            "base.wheel#0",
            "right_hand.finger.flex#2",
            "torso.yaw",
        ] {
            let once = parse_role(Some(role)).unwrap().key();
            let twice = parse_role(Some(&once)).unwrap().key();
            assert_eq!(once, twice);
        }
    }
}

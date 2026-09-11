// The body descriptor's frame graph (§44.3).
//
// Two checks, and the split between them is the point.
//
// `validate_frames` is **graph-only**: it is given a tree and asked whether the tree is well
// formed, and it answers without ever seeing the descriptor the tree came out of. That leaves one
// whole class of defect uncaught, because the two most important names in a dynamic frame —
// `joint` and `mimics` — do not refer to anything in the graph. They refer to the joint list.
//
// `validate_frame_bindings` closes it. A joint name that resolves to nothing presents very far
// from its cause: the kernel binds channels by joint name, finds nothing for that frame, `solve`
// marks it unresolved, and both pose guards trip on it every cycle for as long as the descriptor
// says what it says. That is correct behaviour and it is also useless — the robot holds still and
// the reason is a spelling in a file nothing was checking. §44.5 asks for the refusal to arrive
// **while somebody is still looking at the file**.
//
// It does *not* require that every declared joint have a frame. A partial tree is legitimate — a
// gripper whose fingers nobody modelled is a body with joints and no frames for them — and §44.5
// already has the guard report unavailable naming the frame it wanted. "Complete" means every name
// the graph uses resolves, not that the graph covers the body.

#ifndef SKYNET_OS_EMBODIMENT_HPP
#define SKYNET_OS_EMBODIMENT_HPP

#include "skynet_os/json.hpp"

#include <optional>
#include <string>
#include <vector>

namespace skynet::embodiment {

struct Fault {
  std::string code;
  std::vector<std::string> subjects;
  std::string detail;
};

/// §44.3's graph rules, answered without ever seeing the descriptor the tree came out of.
[[nodiscard]] std::optional<Fault> validate_frames(const Json& graph);

/// Check a frame graph against the joint list of the body it describes.
[[nodiscard]] std::optional<Fault> validate_frame_bindings(const Json& graph, const Json& joints);

/// Does this body declare something that reports person distance? (§5, §7.1's `proximity`.)
[[nodiscard]] bool has_people_detection(const Json& descriptor);

/// `shapeFrameFault` — present and null, so "checked and sound" cannot read as "not checked".
[[nodiscard]] Json fault_to_json(const std::optional<Fault>& fault);

/// A number inside a sentence, rendered as the reference's template literal renders it.
[[nodiscard]] std::string number_in_sentence(const Json& value);

}  // namespace skynet::embodiment

#endif  // SKYNET_OS_EMBODIMENT_HPP

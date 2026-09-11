// The `frames`, `embodiment` and `kinematics` corpora — the body's own geometry.
//
// Three kinds, one file, because all three are views of the same two modules and splitting them
// would put `validate_frames` and the solver that depends on it behind different headings.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/embodiment.hpp"
#include "skynet_os/kinematics.hpp"

#include <string>

namespace skynet::contracts {

Json frames(const std::string& label, const Json& input) {
  // Dispatched on what the label contains rather than on equality, matching the reference runner:
  // the two files are `bindings` and `validate`, and a third would have to be added here rather
  // than falling into whichever arm sorted first.
  if (label.find("bindings") != std::string::npos) {
    return embodiment::fault_to_json(
        embodiment::validate_frame_bindings(input["graph"], input["joints"]));
  }
  if (label.find("validate") != std::string::npos) {
    return embodiment::fault_to_json(embodiment::validate_frames(input["graph"]));
  }
  throw Unimplemented("frames", label);
}

Json embodiment_descriptor(const std::string& /*label*/, const Json& input) {
  // §5's `detects_people`: one derived boolean, off a whole descriptor.
  Json out = Json::object();
  out.set("people_detection", Json::boolean(embodiment::has_people_detection(input)));
  return out;
}

Json kinematics_solved(const std::string& /*label*/, const Json& input) {
  // `shapeSolved` — every frame's pose, present and null rather than absent when unresolved. "This
  // frame has no pose" is the fact the guards act on, so it is rendered rather than omitted: a
  // consumer reading an absent key cannot tell it from a frame nobody mentioned.
  const std::vector<kinematics::Frame> flattened = kinematics::flatten(input["frames"]);

  std::vector<kinematics::Binding> bindings;
  for (const Json& entry : input["joints"].items()) {
    kinematics::Binding binding;
    binding.joint = std::string(entry["joint"].string_or(""));
    if (entry.has("channel") && !entry["channel"].is_null()) {
      binding.channel = std::string(entry["channel"].string_or(""));
    }
    if (entry.has("measured") && !entry["measured"].is_null()) {
      binding.measured = &entry["measured"];
    }
    bindings.push_back(std::move(binding));
  }

  const kinematics::Solved solved = kinematics::solve(flattened, bindings, input["command"]);

  const auto vector_json = [](const auto& values) {
    Json out = Json::array();
    for (const double value : values) {
      out.push(Json::number(value));
    }
    return out;
  };

  Json rows = Json::array();
  for (std::size_t index = 0; index < flattened.size(); ++index) {
    const std::optional<kinematics::Transform>& pose = solved.poses[index];
    Json row = Json::object();
    row.set("name", Json::string(flattened[index].name));
    row.set("resolved", Json::boolean(pose.has_value()));
    row.set("translation", pose.has_value() ? vector_json(pose->translation) : Json::null());
    row.set("rotation", pose.has_value() ? vector_json(pose->rotation) : Json::null());
    rows.push(row);
  }

  Json unresolved = Json::array();
  for (const std::size_t index : solved.unresolved) {
    unresolved.push(Json::string(flattened[index].name));
  }

  Json out = Json::object();
  out.set("frames", rows);
  out.set("lookahead", Json::boolean(solved.lookahead));
  out.set("unresolved", unresolved);
  out.set("collision_pairs",
          Json::number(static_cast<double>(kinematics::collision_pairs(flattened))));
  return out;
}

}  // namespace skynet::contracts

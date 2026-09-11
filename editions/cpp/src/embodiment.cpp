#include "skynet_os/embodiment.hpp"

#include <algorithm>
#include <cmath>
#include <set>

namespace skynet::embodiment {
namespace {

const std::vector<std::string>& frame_kinds() {
  static const std::vector<std::string> kinds = {"root", "static", "dynamic"};
  return kinds;
}

const std::vector<std::string>& collision_shapes() {
  static const std::vector<std::string> shapes = {"capsule"};
  return shapes;
}

Fault fault(std::string code, std::vector<std::string> subjects, std::string detail) {
  return Fault{std::move(code), std::move(subjects), std::move(detail)};
}

std::string name_of(const Json& node) { return std::string(node["name"].string_or("")); }

/// How a template literal renders a value: `${undefined}` is the six characters, `${null}` the
/// four, and a whole-valued double has no fractional part. An edition that printed its own host's
/// spelling of nothing would produce a different sentence for the same input.
std::string in_sentence(const Json* value) {
  if (value == nullptr) {
    return "undefined";
  }
  switch (value->kind()) {
    case Json::Kind::Null: return "null";
    case Json::Kind::Bool: return value->is_true() ? "true" : "false";
    case Json::Kind::Number: return format_number(value->number_or(0));
    case Json::Kind::String: return std::string(value->string_or(""));
    default: return "[object Object]";
  }
}

/// A finite, positive number — and not a boolean. `typeof v === 'number'` is false for a boolean,
/// so an edition that admitted one would read `radius_m: true` as a one-metre capsule.
bool finite_positive(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0)) && value.number_or(0) > 0;
}

}  // namespace

std::string number_in_sentence(const Json& value) { return in_sentence(&value); }

std::optional<Fault> validate_frames(const Json& graph) {
  const JsonArray& nodes = graph["nodes"].items();
  if (nodes.empty()) {
    return fault("empty", {}, "The graph declares no frames.");
  }

  std::set<std::string> seen;
  for (const Json& node : nodes) {
    const std::string name = name_of(node);
    if (seen.count(name) != 0) {
      return fault("duplicate_name", {name},
                   "Two frames are named " + name + ", so a parent reference is ambiguous.");
    }
    seen.insert(name);
  }

  std::vector<std::string> roots;
  for (const Json& node : nodes) {
    if (!node.has("parent") || node["parent"].is_null()) {
      roots.push_back(name_of(node));
    }
  }
  if (roots.size() > 1) {
    return fault("several_roots", roots,
                 std::to_string(roots.size()) +
                     " frames have no parent, so this is a forest and not a tree.");
  }
  const std::string root_name(graph["root"].string_or(""));
  if (seen.count(root_name) == 0) {
    return fault("root_missing", {root_name},
                 "The declared root " + in_sentence(graph.has("root") ? &graph["root"] : nullptr) +
                     " is not one of the frames.");
  }

  for (const Json& node : nodes) {
    const std::string name = name_of(node);
    const std::string kind(node["kind"].string_or(""));
    if (std::find(frame_kinds().begin(), frame_kinds().end(), kind) == frame_kinds().end()) {
      return fault("unknown_kind", {name},
                   "Frame " + name + " has kind " +
                       in_sentence(node.has("kind") ? &node["kind"] : nullptr) +
                       ", which is not root, static or dynamic.");
    }
    const bool has_joint = node.has("joint") && !node["joint"].is_null();
    if ((kind == "dynamic") != has_joint) {
      const std::string detail = kind == "dynamic"
                                     ? "Frame " + name + " is dynamic and names no joint."
                                     : "Frame " + name + " is " + kind + " and names a joint.";
      return fault("joint_mismatch", {name}, detail);
    }
    if (node.has("parent") && !node["parent"].is_null()) {
      const std::string parent(node["parent"].string_or(""));
      if (seen.count(parent) == 0) {
        return fault("parent_missing", {name, parent},
                     "Frame " + name + " names the parent " + parent +
                         ", which is not in the graph.");
      }
    }
  }

  // Every frame reaches the root by following parents. The walk is bounded by the node count, so a
  // cycle is a walk that ran out of frames rather than one that ran forever.
  const auto node_named = [&](const std::string& name) -> const Json* {
    for (const Json& node : nodes) {
      if (name_of(node) == name) {
        return &node;
      }
    }
    return nullptr;
  };
  for (const Json& node : nodes) {
    const Json* here = &node;
    for (std::size_t step = 0; step <= nodes.size(); ++step) {
      if (!here->has("parent") || (*here)["parent"].is_null()) {
        break;
      }
      const Json* next = node_named(std::string((*here)["parent"].string_or("")));
      if (next == nullptr) {
        break;
      }
      here = next;
    }
    if (here->has("parent") && !(*here)["parent"].is_null()) {
      return fault("cycle", {name_of(node)},
                   "Following parents from " + name_of(node) + " never reaches the root.");
    }
  }

  // The collision geometry. Checked here rather than by the guard that consumes it: a shape nobody
  // can read presents as a limb quietly missing from the collision test, and an ignore list naming
  // a frame that does not exist presents as a pair that trips when the operator believed they had
  // excluded it.
  for (const Json& node : nodes) {
    const std::string name = name_of(node);
    if (node.has("collision_shape") && !node["collision_shape"].is_null()) {
      const Json& shape = node["collision_shape"];
      const std::string kind(shape["kind"].string_or(""));
      if (std::find(collision_shapes().begin(), collision_shapes().end(), kind) ==
          collision_shapes().end()) {
        return fault("unknown_shape", {name},
                     "Frame " + name + " declares collision geometry of kind " +
                         in_sentence(shape.has("kind") ? &shape["kind"] : nullptr) +
                         ", which this edition cannot evaluate.");
      }
      if (!finite_positive(shape["radius_m"])) {
        return fault("unknown_shape", {name},
                     "Frame " + name + "'s collision geometry has radius " +
                         in_sentence(shape.has("radius_m") ? &shape["radius_m"] : nullptr) +
                         ". A capsule of no radius bounds nothing, and a guard built on it would "
                         "report clearance it has not established.");
      }
      if (!node["collision"].is_true()) {
        return fault("collision_mismatch", {name},
                     "Frame " + name +
                         " declares a collision shape and says it carries no collision geometry. "
                         "One of the two is wrong and nothing here can tell which.");
      }
    }
    for (const Json& ignored : node["collision_ignore"].items()) {
      const std::string other(ignored.string_or(""));
      if (seen.count(other) == 0) {
        return fault("ignore_missing", {name, other},
                     "Frame " + name + " is permitted to touch " + other +
                         ", which is not one of the frames. An exception naming nothing excludes "
                         "nothing.");
      }
    }
  }

  return std::nullopt;
}

std::optional<Fault> validate_frame_bindings(const Json& graph, const Json& joints) {
  std::set<std::string> declared;
  for (const Json& joint : joints.items()) {
    declared.insert(std::string(joint.string_or("")));
  }

  for (const Json& node : graph["nodes"].items()) {
    const std::string name = name_of(node);
    const bool has_joint = node.has("joint") && !node["joint"].is_null();
    if (node["kind"].string_or("") == "dynamic" && has_joint) {
      const std::string joint(node["joint"].string_or(""));
      if (declared.count(joint) == 0) {
        return fault("dangling_joint", {name, joint},
                     "Frame " + name + " moves with " + joint +
                         ", which the body does not declare. Nothing binds it, so its pose never "
                         "resolves and both pose guards trip on it every cycle.");
      }
    }

    // Presence, not truthiness: an explicit null coefficient is a declared value that is not a
    // number, and the reference tells it apart from an absent one.
    const bool has_coefficient = node.has("mimic_multiplier") || node.has("mimic_offset");
    if (!node.has("mimics") || node["mimics"].is_null()) {
      if (has_coefficient) {
        return fault("mimic_coefficient", {name},
                     "Frame " + name +
                         " declares a mimic multiplier or offset and no `mimics`. A coefficient "
                         "with nothing to multiply describes a linkage that does not exist.");
      }
      continue;
    }

    const std::string mimics(node["mimics"].string_or(""));
    if (declared.count(mimics) == 0) {
      return fault("mimic_missing", {name, mimics},
                   "Frame " + name + " follows " + mimics +
                       ", which the body does not declare. A linkage read from a joint nobody "
                       "drives never resolves, and takes every frame below it with it.");
    }
    if (has_joint && node["joint"].string_or("") == mimics) {
      return fault("mimic_self", {name, mimics},
                   "Frame " + name +
                       " follows the joint it is driven by. `value = multiplier \xc3\x97 driver + "
                       "offset` over one value is a fixed point, not a linkage.");
    }

    for (const char* label : {"mimic_multiplier", "mimic_offset"}) {
      if (!node.has(label)) {
        continue;
      }
      const Json& value = node[label];
      if (!value.is_number() || !std::isfinite(value.number_or(0))) {
        return fault("mimic_coefficient", {name},
                     "Frame " + name + " declares " + label +
                         " as a value that is not a finite number. A linkage computed from it "
                         "produces a pose that is not one.");
      }
    }
  }

  return std::nullopt;
}

bool has_people_detection(const Json& descriptor) {
  // `is_true()`, never truthiness. The reference tests `=== true`, which admits the boolean and
  // nothing else; a bare truthiness test admits every non-empty string — so
  // `detects_people: "no"` would read as a declaration that this sensor detects people.
  // Fail-closed is the only defensible direction on a guard input, and the corpus pins it with
  // `a-declaration-that-is-not-a-boolean`.
  for (const Json& sensor : descriptor["sensors"].items()) {
    if (sensor["detects_people"].is_true()) {
      return true;
    }
  }
  return false;
}

Json fault_to_json(const std::optional<Fault>& fault_value) {
  Json out = Json::object();
  if (!fault_value.has_value()) {
    out.set("fault", Json::null());
    return out;
  }
  Json body = Json::object();
  body.set("code", Json::string(fault_value->code));
  Json subjects = Json::array();
  for (const std::string& subject : fault_value->subjects) {
    subjects.push(Json::string(subject));
  }
  body.set("subjects", subjects);
  body.set("detail", Json::string(fault_value->detail));
  out.set("fault", body);
  return out;
}

}  // namespace skynet::embodiment
